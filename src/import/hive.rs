//! Reading a Windows registry hive file (SOFTWARE, SYSTEM, NTUSER.DAT), for
//! `nami import` from a USB drive next to Windows: keys and their text and
//! number values, nothing more. The format ("regf") is a header block,
//! then 4 KiB bins of cells: keys (nk), their subkey lists (lf, lh, li,
//! ri), values (vk) and data. Offsets count from the first bin.

use anyhow::{bail, ensure, Result};

pub struct Hive {
    data: Vec<u8>,
    root: u32,
}

/// A key in the hive (an offset to its nk cell).
#[derive(Clone, Copy)]
pub struct Key(u32);

#[derive(Debug, PartialEq)]
pub enum Data {
    Text(String),
    Number(u64),
    Other,
}

const BINS: usize = 4096;

impl Hive {
    pub fn parse(data: Vec<u8>) -> Result<Hive> {
        ensure!(data.len() > BINS && &data[..4] == b"regf", "not a registry hive");
        let root = u32::from_le_bytes(data[0x24..0x28].try_into().unwrap());
        let h = Hive { data, root };
        ensure!(h.cell(root).is_some_and(|c| c.starts_with(b"nk")), "the hive's root key is missing");
        Ok(h)
    }

    pub fn root(&self) -> Key {
        Key(self.root)
    }

    /// A cell's data (after its size), if it's in the file.
    fn cell(&self, off: u32) -> Option<&[u8]> {
        let at = BINS.checked_add(off as usize)?;
        let size = i32::from_le_bytes(self.data.get(at..at + 4)?.try_into().ok()?);
        let len = size.unsigned_abs() as usize;
        if len < 4 {
            return None;
        }
        self.data.get(at + 4..at + len)
    }

    fn u16(b: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
    }
    fn u32(b: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    }

    fn name(raw: &[u8], ascii: bool) -> String {
        if ascii {
            raw.iter().map(|&b| b as char).collect()
        } else {
            utf16(raw)
        }
    }

    pub fn key_name(&self, k: Key) -> String {
        let Some(c) = self.cell(k.0) else { return String::new() };
        let len = Self::u16(c, 0x48).unwrap_or(0) as usize;
        let flags = Self::u16(c, 0x02).unwrap_or(0);
        c.get(0x4C..0x4C + len).map(|raw| Self::name(raw, flags & 0x20 != 0)).unwrap_or_default()
    }

    /// A key's subkeys.
    pub fn subkeys(&self, k: Key) -> Vec<Key> {
        let mut out = Vec::new();
        if let Some(c) = self.cell(k.0) {
            if Self::u32(c, 0x14).unwrap_or(0) > 0 {
                if let Some(list) = Self::u32(c, 0x1C) {
                    self.list(list, &mut out, 0);
                }
            }
        }
        out
    }

    fn list(&self, off: u32, out: &mut Vec<Key>, depth: usize) {
        let Some(c) = self.cell(off) else { return };
        if depth > 4 || c.len() < 4 {
            return;
        }
        let n = Self::u16(c, 2).unwrap_or(0) as usize;
        match &c[..2] {
            b"lf" | b"lh" => out.extend((0..n).filter_map(|i| Self::u32(c, 4 + i * 8)).map(Key)),
            b"li" => out.extend((0..n).filter_map(|i| Self::u32(c, 4 + i * 4)).map(Key)),
            b"ri" => {
                for i in 0..n {
                    if let Some(o) = Self::u32(c, 4 + i * 4) {
                        self.list(o, out, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    /// The subkey with this name (any case, as Windows compares them).
    pub fn child(&self, k: Key, name: &str) -> Option<Key> {
        self.subkeys(k).into_iter().find(|s| self.key_name(*s).eq_ignore_ascii_case(name))
    }

    /// A key by its path from the root, `Microsoft\Windows\CurrentVersion`.
    pub fn open(&self, path: &str) -> Option<Key> {
        path.split('\\').filter(|p| !p.is_empty()).try_fold(self.root(), |k, part| self.child(k, part))
    }

    /// A value of a key, by name (any case; "" is the key's default value).
    pub fn value(&self, k: Key, name: &str) -> Option<Data> {
        let c = self.cell(k.0)?;
        let count = Self::u32(c, 0x24)? as usize;
        if count == 0 {
            return None;
        }
        let list = self.cell(Self::u32(c, 0x28)?)?;
        for i in 0..count.min(list.len() / 4) {
            let vk = self.cell(Self::u32(list, i * 4)?)?;
            if !vk.starts_with(b"vk") {
                continue;
            }
            let len = Self::u16(vk, 2)? as usize;
            let flags = Self::u16(vk, 0x10)?;
            let vname = vk.get(0x14..0x14 + len).map(|raw| Self::name(raw, flags & 1 != 0)).unwrap_or_default();
            if !vname.eq_ignore_ascii_case(name) {
                continue;
            }
            return Some(self.data_of(vk));
        }
        None
    }

    fn data_of(&self, vk: &[u8]) -> Data {
        let (Some(size), Some(off), Some(kind)) = (Self::u32(vk, 4), Self::u32(vk, 8), Self::u32(vk, 12)) else { return Data::Other };
        let inline = size & 0x8000_0000 != 0;
        let size = (size & 0x7fff_ffff) as usize;
        let bytes: Vec<u8> = if inline {
            off.to_le_bytes()[..size.min(4)].to_vec()
        } else {
            match self.cell(off) {
                Some(c) if c.len() >= size => c[..size].to_vec(),
                _ => return Data::Other,
            }
        };
        match kind {
            1 | 2 => Data::Text(utf16(&bytes)),
            4 if bytes.len() >= 4 => Data::Number(u32::from_le_bytes(bytes[..4].try_into().unwrap()) as u64),
            11 if bytes.len() >= 8 => Data::Number(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
            _ => Data::Other,
        }
    }

    pub fn text(&self, k: Key, name: &str) -> Option<String> {
        match self.value(k, name)? {
            Data::Text(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        }
    }
}

/// UTF-16LE up to its first NUL.
fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

pub fn read(path: &std::path::Path) -> Result<Hive> {
    match std::fs::read(path) {
        Ok(d) => Hive::parse(d),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => bail!("Windows is running and holds it, or it needs root"),
        Err(e) => bail!("{e}"),
    }
}

/// A small hive written from keys and values, for tests.
#[cfg(test)]
pub mod build {
    pub enum V<'a> {
        Text(&'a str),
        Number(u32),
    }
    pub struct K<'a> {
        pub name: &'a str,
        pub values: Vec<(&'a str, V<'a>)>,
        pub keys: Vec<K<'a>>,
    }

    struct W {
        bins: Vec<u8>,
    }
    impl W {
        fn cell(&mut self, data: &[u8]) -> u32 {
            let off = self.bins.len() as u32;
            let len = (data.len() + 4).div_ceil(8) * 8;
            self.bins.extend((-(len as i32)).to_le_bytes());
            self.bins.extend(data);
            self.bins.resize(off as usize + len, 0);
            off
        }
        fn set_u32(&mut self, at: usize, v: u32) {
            self.bins[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        fn key(&mut self, k: &K) -> u32 {
            let mut nk = vec![0u8; 0x4C];
            nk[..2].copy_from_slice(b"nk");
            nk[2..4].copy_from_slice(&0x20u16.to_le_bytes());
            nk[0x48..0x4A].copy_from_slice(&(k.name.len() as u16).to_le_bytes());
            nk.extend(k.name.as_bytes());
            let me = self.cell(&nk);
            let subs: Vec<u32> = k.keys.iter().map(|s| self.key(s)).collect();
            if !subs.is_empty() {
                let mut lf = b"lf".to_vec();
                lf.extend((subs.len() as u16).to_le_bytes());
                for s in &subs {
                    lf.extend(s.to_le_bytes());
                    lf.extend([0u8; 4]);
                }
                let list = self.cell(&lf);
                self.set_u32(me as usize + 4 + 0x14, subs.len() as u32);
                self.set_u32(me as usize + 4 + 0x1C, list);
            }
            if !k.values.is_empty() {
                let mut offs = Vec::new();
                for (name, v) in &k.values {
                    let (kind, bytes): (u32, Vec<u8>) = match v {
                        V::Text(s) => (1, s.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect()),
                        V::Number(n) => (4, n.to_le_bytes().to_vec()),
                    };
                    let mut vk = b"vk".to_vec();
                    vk.extend((name.len() as u16).to_le_bytes());
                    if kind == 4 {
                        vk.extend((4u32 | 0x8000_0000).to_le_bytes());
                        vk.extend(&bytes[..4]);
                    } else {
                        let data = self.cell(&bytes);
                        vk.extend((bytes.len() as u32).to_le_bytes());
                        vk.extend(data.to_le_bytes());
                    }
                    vk.extend(kind.to_le_bytes());
                    vk.extend(1u16.to_le_bytes());
                    vk.extend([0u8; 2]);
                    vk.extend(name.as_bytes());
                    offs.push(self.cell(&vk));
                }
                let list: Vec<u8> = offs.iter().flat_map(|o| o.to_le_bytes()).collect();
                let list = self.cell(&list);
                self.set_u32(me as usize + 4 + 0x24, offs.len() as u32);
                self.set_u32(me as usize + 4 + 0x28, list);
            }
            me
        }
    }

    pub fn hive(root: &K) -> Vec<u8> {
        let mut w = W { bins: b"hbin".to_vec() };
        w.bins.resize(0x20, 0);
        let r = w.key(root);
        let mut out = vec![0u8; super::BINS];
        out[..4].copy_from_slice(b"regf");
        out[0x24..0x28].copy_from_slice(&r.to_le_bytes());
        out.extend(w.bins);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::{hive, K, V};
    use super::*;

    #[test]
    fn reads_keys_and_values() {
        let data = hive(&K {
            name: "ROOT",
            values: vec![],
            keys: vec![K {
                name: "Microsoft",
                values: vec![],
                keys: vec![K { name: "Uninstall", values: vec![], keys: vec![K { name: "{abc}", values: vec![("DisplayName", V::Text("Spotify")), ("SystemComponent", V::Number(1))], keys: vec![] }] }],
            }],
        });
        let h = Hive::parse(data).unwrap();
        let u = h.open("microsoft\\Uninstall").unwrap();
        let apps = h.subkeys(u);
        assert_eq!(apps.len(), 1);
        assert_eq!(h.key_name(apps[0]), "{abc}");
        assert_eq!(h.text(apps[0], "displayname").as_deref(), Some("Spotify"));
        assert_eq!(h.value(apps[0], "SystemComponent"), Some(Data::Number(1)));
        assert!(h.open("Microsoft\\Nope").is_none());
        assert!(Hive::parse(b"nope".repeat(2000)).is_err());
    }
}
