//! What a program or library links to: the dynamic section of a 64-bit
//! ELF file (what `readelf -d` and `readelf -V` print).

use std::collections::BTreeMap;

/// A program's or library's dynamic linking.
#[derive(Debug, Default, PartialEq)]
pub struct Linking {
    /// The libraries it links to (NEEDED).
    pub needed: Vec<String>,
    /// Its own name, for a library (SONAME).
    pub soname: Option<String>,
    /// Whether it's a program with a loader (an interpreter), not a library.
    pub program: bool,
    /// The symbol versions it needs, by library: `libcurl.so.4` ->
    /// `[CURL_OPENSSL_4]` (the verneed section).
    pub versions_needed: BTreeMap<String, Vec<String>>,
    /// The symbol versions a library defines (verdef), its own name aside.
    pub versions: Vec<String>,
    /// Where it says its libraries are, before LD_LIBRARY_PATH (RPATH) and
    /// after (RUNPATH): folders separated by `:`, `$ORIGIN` its own.
    pub rpath: Option<String>,
    pub runpath: Option<String>,
}

/// A 64-bit little-endian ELF file's linking. None for anything else
/// (scripts, 32-bit programs, static ones).
pub fn linking(data: &[u8]) -> Option<Linking> {
    if data.len() < 64 || &data[..4] != b"\x7fELF" || data[4] != 2 || data[5] != 1 {
        return None;
    }
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let u64_at = |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let (phoff, phentsize, phnum) = (u64_at(0x20)? as usize, u16_at(0x36)?, u16_at(0x38)?);
    // Program headers: the dynamic section, and the loaded segments that
    // turn its addresses into file offsets.
    let (mut dynamic, mut loads, mut program) = (None, Vec::new(), false);
    for i in 0..phnum {
        let h = phoff + i * phentsize;
        let kind = u32_at(h)?;
        let (offset, vaddr, filesz) = (u64_at(h + 8)?, u64_at(h + 16)?, u64_at(h + 32)?);
        match kind {
            1 => loads.push((offset, vaddr, filesz)),
            2 => dynamic = Some((offset as usize, filesz as usize)),
            3 => program = true,
            _ => {}
        }
    }
    let (start, size) = dynamic?;
    let (mut needed, mut soname, mut strtab, mut rpath, mut runpath) = (Vec::new(), None, None, None, None);
    let (mut verneed, mut verneednum, mut verdef, mut verdefnum) = (None, 0, None, 0);
    for at in (start..start + size).step_by(16) {
        let (tag, val) = (u64_at(at)?, u64_at(at + 8)?);
        match tag {
            0 => break,
            1 => needed.push(val),
            5 => strtab = Some(val),
            14 => soname = Some(val),
            15 => rpath = Some(val),
            29 => runpath = Some(val),
            0x6ffffffc => verdef = Some(val),
            0x6ffffffd => verdefnum = val,
            0x6ffffffe => verneed = Some(val),
            0x6fffffff => verneednum = val,
            _ => {}
        }
    }
    let offset = |addr: u64| loads.iter().find(|(_, v, s)| addr >= *v && addr < v + s).map(|(o, v, _)| (o + (addr - v)) as usize);
    let table = offset(strtab?)?;
    let string = |off: usize| {
        let bytes = data.get(table + off..)?;
        let end = bytes.iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    };
    let mut out = Linking {
        needed: needed.into_iter().filter_map(|n| string(n as usize)).collect(),
        soname: soname.and_then(|s| string(s as usize)),
        rpath: rpath.and_then(|s| string(s as usize)),
        runpath: runpath.and_then(|s| string(s as usize)),
        program,
        ..Default::default()
    };
    // Verneed: a list of files, each with a list of versions.
    let mut at = verneed.and_then(offset);
    for _ in 0..verneednum.min(1000) {
        let Some(n) = at else { break };
        let (count, file, aux, next) = (u16_at(n + 2)?, u32_at(n + 4)?, u32_at(n + 8)?, u32_at(n + 12)?);
        let mut versions = Vec::new();
        let mut a = n + aux;
        for _ in 0..count {
            versions.extend(string(u32_at(a + 8)?));
            match u32_at(a + 12)? {
                0 => break,
                step => a += step,
            }
        }
        out.versions_needed.entry(string(file)?).or_default().extend(versions);
        at = (next != 0).then_some(n + next);
    }
    // Verdef: the versions it defines; the first is its own name.
    let mut at = verdef.and_then(offset);
    for _ in 0..verdefnum.min(1000) {
        let Some(d) = at else { break };
        let (flags, aux, next) = (u16_at(d + 2)?, u32_at(d + 12)?, u32_at(d + 16)?);
        if flags & 1 == 0 {
            out.versions.extend(string(u32_at(d + aux)?));
        }
        at = (next != 0).then_some(d + next);
    }
    Some(out)
}

/// The symbols a library defines for others: its dynamic symbol table's
/// global and weak ones that aren't undefined (`nm -D --defined-only`).
pub fn exported(data: &[u8]) -> Option<std::collections::BTreeSet<String>> {
    if data.len() < 64 || &data[..4] != b"\x7fELF" || data[4] != 2 || data[5] != 1 {
        return None;
    }
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let u64_at = |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize);
    let (shoff, shentsize, shnum) = (u64_at(0x28)?, u16_at(0x3a)?, u16_at(0x3c)?);
    let section = |i: usize| shoff + i * shentsize;
    let dynsym = (0..shnum).map(section).find(|h| u32_at(h + 4) == Some(11))?;
    let (offset, size, link) = (u64_at(dynsym + 0x18)?, u64_at(dynsym + 0x20)?, u32_at(dynsym + 0x28)?);
    let strings = u64_at(section(link) + 0x18)?;
    let mut out = std::collections::BTreeSet::new();
    for at in (offset..offset + size).step_by(24) {
        let (name, info, shndx) = (u32_at(at)?, *data.get(at + 4)?, u16_at(at + 6)?);
        if name == 0 || shndx == 0 || !matches!(info >> 4, 1 | 2) {
            continue;
        }
        let bytes = data.get(strings + name..)?;
        let end = bytes.iter().position(|b| *b == 0)?;
        out.insert(String::from_utf8_lossy(&bytes[..end]).into_owned());
    }
    Some(out)
}

/// Library names spelled out in a file (`libGL.so.1`): what a program may
/// load with dlopen at runtime.
pub fn mentioned_libraries(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let ok = |b: u8| b.is_ascii_alphanumeric() || b"_+-.".contains(&b);
    let mut i = 0;
    while let Some(at) = data[i..].windows(3).position(|w| w == b"lib").map(|p| p + i) {
        if at > 0 && ok(data[at - 1]) {
            i = at + 3;
            continue;
        }
        let end = at + data[at..].iter().position(|b| !ok(*b)).unwrap_or(data.len() - at);
        let word = String::from_utf8_lossy(&data[at..end]);
        if let Some(pos) = word.find(".so") {
            let tail = &word[pos + 3..];
            if pos > 3 && (tail.is_empty() || (tail.starts_with('.') && tail[1..].split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))) {
                out.push(word.into_owned());
            }
        }
        i = end.max(at + 3);
    }
    out.sort();
    out.dedup();
    out
}

/// Absolute paths under /usr and /opt spelled out in a file, each its own
/// string (`/usr/share/foo/data.csv`): where a program looks for its files.
pub fn mentioned_paths(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let ok = |b: u8| b.is_ascii_alphanumeric() || b"_+-./@".contains(&b);
    for prefix in [&b"/usr/"[..], b"/opt/"] {
        let mut i = 0;
        while let Some(at) = data[i..].windows(5).position(|w| w == prefix).map(|p| p + i) {
            let end = at + data[at..].iter().position(|b| !ok(*b)).unwrap_or(data.len() - at);
            // A string of its own: not the middle of a word or a longer path.
            if at == 0 || !ok(data[at - 1]) {
                out.push(String::from_utf8_lossy(&data[at..end]).into_owned());
            }
            i = end.max(at + 5);
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "reads this test program as an ELF file and glibc")]
    fn reads_what_a_program_links_to() {
        // nami itself is a dynamically linked 64-bit program.
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let l = linking(&me).expect("an ELF file");
        assert!(l.needed.iter().any(|n| n == "libc.so.6"), "{l:?}");
        assert!(l.program);
        // It needs glibc's versioned symbols.
        assert!(l.versions_needed["libc.so.6"].iter().any(|v| v.starts_with("GLIBC_2.")), "{l:?}");
        assert!(linking(b"#!/bin/sh\necho hi\n").is_none());
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "reads this test program as an ELF file and glibc")]
    fn reads_what_a_library_defines() {
        // The C library the tests run with defines GLIBC_2.2.5 and on (on ARM, GLIBC_2.17: its first).
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let libc = maps.lines().filter_map(|l| l.split_whitespace().nth(5)).find(|p| p.ends_with("/libc.so.6")).expect("libc in this process");
        let l = linking(&std::fs::read(libc).unwrap()).unwrap();
        assert_eq!(l.soname.as_deref(), Some("libc.so.6"));
        let first = if cfg!(target_arch = "aarch64") { "GLIBC_2.17" } else { "GLIBC_2.2.5" };
        assert!(l.versions.iter().any(|v| v == first), "{:?}", l.versions);
        assert!(!l.versions.iter().any(|v| v == "libc.so.6"));
    }

    #[test]
    fn finds_dlopened_names() {
        let data = b"\0libGL.so.1\0xlibfoo.so.2\0libvulkan.so.1\0libc.so.6x\0libbar.so\0lib.so.1\0libz.so.1.2.3\0libq.so.a\0";
        assert_eq!(mentioned_libraries(data), vec!["libGL.so.1", "libbar.so", "libvulkan.so.1", "libz.so.1.2.3"]);
    }

    #[test]
    fn finds_paths() {
        let data = b"\0/usr/share/distro-info/ubuntu.csv\0See /usr/share/doc/x/README for more\0x/usr/lib/no\0/opt/Foo/\0/usr/\0";
        assert_eq!(mentioned_paths(data), vec!["/opt/Foo/", "/usr/", "/usr/share/distro-info/ubuntu.csv", "/usr/share/doc/x/README"]);
    }
}

#[cfg(test)]
mod exported_tests {
    /// A real library's symbols, when one is at hand (`NAMI_TEST_SO`).
    #[test]
    fn a_libraries_symbols() {
        let Some(path) = std::env::var_os("NAMI_TEST_SO") else { return };
        let data = std::fs::read(path).unwrap();
        let s = super::exported(&data).unwrap();
        eprintln!("{} symbols, soname {:?}", s.len(), super::linking(&data).unwrap().soname);
        assert!(s.contains("inflate") && s.contains("deflate") && !s.contains("memcpy"), "{s:?}");
    }
}
