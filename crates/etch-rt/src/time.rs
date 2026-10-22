//! Times, formatted once when a fragment is built: in the server's time zone (TZ, fixed at start),
//! as crudspec's `<Time>` writes them.

use jiff::tz::TimeZone;
use jiff::Timestamp;
use std::sync::OnceLock;

fn zone() -> &'static TimeZone {
    static Z: OnceLock<TimeZone> = OnceLock::new();
    Z.get_or_init(|| TimeZone::system())
}

/// "2026-02-28T18:44:00.000Z"
pub fn iso_utc(ms: i64) -> String {
    let t = Timestamp::from_millisecond(ms).expect("time in range");
    t.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// "1:44\u{202f}PM"
pub fn clock(ms: i64) -> String {
    let z = Timestamp::from_millisecond(ms).expect("time in range").to_zoned(zone().clone());
    let h = z.hour();
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{}:{:02}\u{202f}{}", h12, z.minute(), if h < 12 { "AM" } else { "PM" })
}

/// "Sat Feb 28 2026"
pub fn date(ms: i64) -> String {
    let z = Timestamp::from_millisecond(ms).expect("time in range").to_zoned(zone().clone());
    z.strftime("%a %b %d %Y").to_string()
}

/// The time now, in milliseconds since 1970.
pub fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}

/// A duration in its two largest units, truncated: "2d 3h", "1h 5m", "4m 20s", "45s"; under a
/// second, "0s"; negative, "-5m".
pub fn span(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let mut left = ms.unsigned_abs() / 1000;
    let mut parts = vec![];
    for (unit, size) in [("d", 86_400u64), ("h", 3_600), ("m", 60), ("s", 1)] {
        let n = left / size;
        left %= size;
        if n > 0 || !parts.is_empty() {
            parts.push(format!("{n}{unit}"));
        }
        if parts.len() == 2 {
            break;
        }
    }
    // a zero second unit is left off ("2h", not "2h 0m")
    if parts.len() == 2 && parts[1].starts_with('0') {
        parts.pop();
    }
    if parts.is_empty() {
        return "0s".into();
    }
    format!("{sign}{}", parts.join(" "))
}

/// An Int with its thousands grouped: "1,234,567", "-1,000".
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut s = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        s.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            s.push(',');
        }
        s.push(c);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc() {
        assert_eq!(iso_utc(1772304240000), "2026-02-28T18:44:00.000Z");
    }

    #[test]
    fn spans() {
        assert_eq!(span(0), "0s");
        assert_eq!(span(999), "0s");
        assert_eq!(span(45_000), "45s");
        assert_eq!(span(260_000), "4m 20s");
        assert_eq!(span(3_900_000), "1h 5m");
        assert_eq!(span(7_200_000), "2h");
        assert_eq!(span(7_201_000), "2h");
        assert_eq!(span(183_600_000), "2d 3h");
        assert_eq!(span(-300_000), "-5m");
        assert_eq!(span(i64::MAX), "106751991167d 7h");
    }

    #[test]
    fn groups() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(-1234567), "-1,234,567");
        assert_eq!(grouped(i64::MIN + 1), "-9,223,372,036,854,775,807");
    }
}
