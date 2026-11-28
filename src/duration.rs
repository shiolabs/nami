//! How long, as nami's files and commands write it: a number and a unit
//! (`300s`, `5m`, `12h`, `30d`, `2w`), or a number alone for seconds. One
//! reading for every place that takes one: a service's `ready-timeout:`,
//! `nami gc --older-than`.

/// What a duration looks like, for messages.
pub const EXAMPLES: &str = "300s, 5m, 12h, 30d or 2w";

/// `text` in seconds. Units: s, m (minutes), h, d, w; none is seconds.
pub fn seconds(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let at = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (num, unit) = text.split_at(at);
    let bad = || format!("{text:?} isn't a duration (like {EXAMPLES})");
    let n: u64 = num.parse().map_err(|_| bad())?;
    let mult = match unit.trim() {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 7 * 86400,
        _ => return Err(format!("{text:?}: the unit is s, m, h, d or w (like {EXAMPLES})")),
    };
    n.checked_mul(mult).ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_reading_for_every_duration() {
        assert_eq!(seconds("300").unwrap(), 300);
        assert_eq!(seconds("300s").unwrap(), 300);
        assert_eq!(seconds("5m").unwrap(), 300);
        assert_eq!(seconds("12h").unwrap(), 12 * 3600);
        assert_eq!(seconds("30d").unwrap(), 30 * 86400);
        assert_eq!(seconds("2w").unwrap(), 14 * 86400);
        assert_eq!(seconds(" 5 m ").unwrap(), 300);
        assert!(seconds("3x").unwrap_err().contains("s, m, h, d or w"));
        assert!(seconds("m").is_err());
        assert!(seconds("").is_err());
        assert!(seconds("-5m").is_err());
    }
}
