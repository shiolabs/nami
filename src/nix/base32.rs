//! Nix's base32 variant: custom alphabet, least-significant 5 bits first.

const ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

pub fn encode(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let len = (bytes.len() * 8 - 1) / 5 + 1;
    let mut out = String::with_capacity(len);
    for n in (0..len).rev() {
        let b = n * 5;
        let (i, j) = (b / 8, b % 8);
        let mut c = (bytes[i] as u16) >> j;
        if i + 1 < bytes.len() {
            c |= (bytes[i + 1] as u16) << (8 - j);
        }
        out.push(ALPHABET[(c & 0x1f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    #[test]
    fn sha256_of_empty_string() {
        // `nix hash file --type sha256 --base32 /dev/null`
        let h = Sha256::digest(b"");
        assert_eq!(
            super::encode(&h),
            "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73"
        );
    }
}
