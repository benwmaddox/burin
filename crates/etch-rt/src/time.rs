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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc() {
        assert_eq!(iso_utc(1772304240000), "2026-02-28T18:44:00.000Z");
    }
}
