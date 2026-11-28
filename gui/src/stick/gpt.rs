//! A disk image's partition table (GPT): where namiOS's image has its boot partition and its
//! root. For the writer (write.rs: your files onto the USB drive's EFI partition) and for
//! namiOS next to Windows (the installer's beside.rs).

/// GPT's types: an EFI system partition, and a Linux root (x86-64).
pub const ESP_TYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
pub const ROOT_TYPE: &str = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709";

/// A stretch of the image: where, and how long (bytes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub start: u64,
    pub size: u64,
}

/// The image's EFI partition and root, from its partition table (GPT, in
/// its first MiB).
pub fn image_parts(head: &[u8]) -> Result<(Region, Region), String> {
    let bad = || "The namiOS download has an unexpected partition table. Try again.".to_string();
    let u32_at = |o: usize| head.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(bad);
    let u64_at = |o: usize| head.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).ok_or_else(bad);
    if head.get(512..520) != Some(b"EFI PART") {
        return Err(bad());
    }
    let (at, count, each) = (u64_at(512 + 72)? as usize * 512, u32_at(512 + 80)? as usize, u32_at(512 + 84)? as usize);
    let (mut esp, mut root) = (None, None);
    for i in 0..count.min(128) {
        let e = at + i * each;
        let kind = guid(head.get(e..e + 16).ok_or_else(bad)?);
        let r = Region { start: u64_at(e + 32)? * 512, size: (u64_at(e + 40)? + 1 - u64_at(e + 32)?) * 512 };
        match kind.as_str() {
            ESP_TYPE => esp = Some(r),
            ROOT_TYPE => root = Some(r),
            _ => {}
        }
    }
    match (esp, root) {
        (Some(e), Some(r)) if e.start + e.size <= r.start => Ok((e, r)),
        _ => Err(bad()),
    }
}

/// A GUID as GPT stores it (its first three parts little-endian).
fn guid(b: &[u8]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}
