//! etch's standard library (lang/std.etch): each function called as an external function (the C
//! ABI: text as UTF-8 bytes and a length, numbers as i64, a text result written to `out`). Text
//! is counted in characters (Unicode scalar values), as SQLite counts it; the functions with a SQL
//! form (lang/std_sql.rs) must give the same answers as it (the test below).

use crate::ext::{text, write, Out};

const NONE: i64 = i64::MIN;
/// The most a function's text result grows to (repeat, pad).
const MAX_OUT: usize = 1 << 20;

fn b(x: bool) -> i64 {
    x as i64
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

// ---- the functions, in plain Rust (tested against SQLite below)

pub fn length(t: &str) -> i64 {
    t.chars().count() as i64
}
pub fn index_of(t: &str, part: &str) -> i64 {
    match t.find(part) {
        Some(at) => t[..at].chars().count() as i64,
        None => -1,
    }
}
pub fn trim(t: &str) -> &str {
    t.trim_matches(is_space)
}
pub fn truncate(t: &str, n: i64) -> String {
    let n = n.max(0) as usize;
    match t.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", &t[..at]),
        None => t.to_string(),
    }
}
pub fn slice(t: &str, start: i64, count: i64) -> String {
    t.chars().skip(start.max(0) as usize).take(count.max(0) as usize).collect()
}
pub fn replace_all(t: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        t.to_string()
    } else {
        t.replace(from, to)
    }
}
pub fn repeat(t: &str, n: i64) -> String {
    if n <= 0 || t.is_empty() {
        return String::new();
    }
    let times = (n as usize).min(MAX_OUT / t.len().max(1));
    t.repeat(times)
}
fn pad(t: &str, width: i64, fill: &str, start: bool) -> String {
    let have = t.chars().count();
    let width = (width.max(0) as usize).min(MAX_OUT);
    if have >= width {
        return t.to_string();
    }
    let c = fill.chars().next().unwrap_or(' ');
    let padding: String = std::iter::repeat_n(c, width - have).collect();
    if start {
        padding + t
    } else {
        t.to_string() + &padding
    }
}
pub fn capitalize(t: &str) -> String {
    let mut cs = t.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().chain(cs).collect(),
        None => String::new(),
    }
}
pub fn decimal(n: i64, places: i64) -> String {
    if n == NONE {
        return String::new();
    }
    let places = places.clamp(0, 18) as u32;
    if places == 0 {
        return n.to_string();
    }
    let scale = 10u128.pow(places);
    let a = n.unsigned_abs() as u128;
    format!("{}{}.{:0width$}", if n < 0 { "-" } else { "" }, a / scale, a % scale, width = places as usize)
}
pub fn percent(part: i64, whole: i64) -> i64 {
    if part == NONE || whole == NONE || whole == 0 {
        return NONE;
    }
    part.checked_mul(100).and_then(|p| p.checked_div(whole)).unwrap_or(NONE)
}
pub fn round_to(n: i64, step: i64) -> i64 {
    if n == NONE || step <= 0 {
        return n;
    }
    let (q, r) = (n / step, n % step);
    // halves away from zero
    let up = r.unsigned_abs() * 2 >= step.unsigned_abs();
    let k = if up { q + n.signum() } else { q };
    k.checked_mul(step).unwrap_or(n)
}

// ---- as external functions

/// # Safety: as for every external function (crate::ext).
#[no_mangle]
pub unsafe extern "C" fn etch_std_length(p: *const u8, n: i64) -> i64 {
    length(&text(p, n))
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_contains(p: *const u8, n: i64, q: *const u8, m: i64) -> i64 {
    b(text(p, n).contains(&*text(q, m)))
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_starts_with(p: *const u8, n: i64, q: *const u8, m: i64) -> i64 {
    b(text(p, n).starts_with(&*text(q, m)))
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_ends_with(p: *const u8, n: i64, q: *const u8, m: i64) -> i64 {
    b(text(p, n).ends_with(&*text(q, m)))
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_index_of(p: *const u8, n: i64, q: *const u8, m: i64) -> i64 {
    index_of(&text(p, n), &text(q, m))
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_trim(p: *const u8, n: i64, out: *mut Out) {
    write(out, trim(&text(p, n)));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_truncate(p: *const u8, n: i64, k: i64, out: *mut Out) {
    write(out, &truncate(&text(p, n), k));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_slice(p: *const u8, n: i64, start: i64, count: i64, out: *mut Out) {
    write(out, &slice(&text(p, n), start, count));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_replace_all(p: *const u8, n: i64, f: *const u8, fl: i64, t: *const u8, tl: i64, out: *mut Out) {
    write(out, &replace_all(&text(p, n), &text(f, fl), &text(t, tl)));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_repeat(p: *const u8, n: i64, times: i64, out: *mut Out) {
    write(out, &repeat(&text(p, n), times));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_pad_start(p: *const u8, n: i64, width: i64, f: *const u8, fl: i64, out: *mut Out) {
    write(out, &pad(&text(p, n), width, &text(f, fl), true));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_pad_end(p: *const u8, n: i64, width: i64, f: *const u8, fl: i64, out: *mut Out) {
    write(out, &pad(&text(p, n), width, &text(f, fl), false));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_capitalize(p: *const u8, n: i64, out: *mut Out) {
    write(out, &capitalize(&text(p, n)));
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_plural(k: i64, p: *const u8, n: i64, q: *const u8, m: i64, out: *mut Out) {
    write(out, &if k == 1 { text(p, n) } else { text(q, m) });
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_decimal(n: i64, places: i64, out: *mut Out) {
    write(out, &decimal(n, places));
}
#[no_mangle]
pub extern "C" fn etch_std_percent(part: i64, whole: i64) -> i64 {
    percent(part, whole)
}
#[no_mangle]
pub extern "C" fn etch_std_round_to(n: i64, step: i64) -> i64 {
    round_to(n, step)
}
/// # Safety: as for every external function.
#[no_mangle]
pub unsafe extern "C" fn etch_std_format_time(ms: i64, p: *const u8, n: i64, out: *mut Out) {
    if ms == NONE {
        return;
    }
    if let Some(s) = crate::time::format(ms, &text(p, n)) {
        write(out, &s);
    }
}
#[no_mangle]
pub extern "C" fn etch_std_weekday(ms: i64) -> i64 {
    if ms == NONE {
        return NONE;
    }
    crate::time::weekday(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    include!("../../../lang/std_sql.rs");

    /// Random text from an alphabet with what matters: ASCII, spaces of each kind, multi-byte
    /// characters, an emoji (4 bytes), the "…" truncate adds, and repeats (so parts are found).
    fn texts(seed: u64, n: usize) -> Vec<String> {
        const ALPHABET: &[&str] = &["a", "b", "A", " ", "\t", "\n", "é", "ü", "日", "😀", "…", "ab", "x"];
        let mut s = seed | 1;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let mut out = vec![String::new()];
        for _ in 0..n {
            let len = (next() % 7) as usize;
            out.push((0..len).map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize]).collect());
        }
        out
    }

    /// Each function with a SQL form gives what SQLite gives, on every input tried.
    #[test]
    fn the_sql_forms_agree_with_the_rust() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        let ts = texts(7, 60);
        let ns: Vec<i64> = vec![-3, -1, 0, 1, 2, 3, 5, 100];
        let sql = |name: &str| STD_SQL.iter().find(|(n, _)| *n == name).unwrap().1;
        let one = |form: &str, args: &[&dyn rusqlite::ToSql]| -> rusqlite::types::Value {
            let mut q = form.to_string();
            for i in (1..=args.len()).rev() {
                q = q.replace(&format!("${i}"), &format!("?{i}"));
            }
            c.query_row(&format!("SELECT {q}"), args, |r| r.get(0)).unwrap()
        };
        use rusqlite::types::Value::{Integer as I, Text as T};
        let mut checked = 0;
        for a in &ts {
            assert_eq!(one(sql("length"), &[a]), I(length(a)), "length({a:?})");
            assert_eq!(one(sql("trim"), &[a]), T(trim(a).to_string()), "trim({a:?})");
            for k in &ns {
                assert_eq!(one(sql("truncate"), &[a, k]), T(truncate(a, *k)), "truncate({a:?}, {k})");
                assert_eq!(one(sql("plural"), &[k, a, &"many"]), T(if *k == 1 { a.clone() } else { "many".into() }), "plural({k}, {a:?})");
                for j in &ns {
                    assert_eq!(one(sql("slice"), &[a, k, j]), T(slice(a, *k, *j)), "slice({a:?}, {k}, {j})");
                }
            }
            for p in &ts {
                assert_eq!(one(sql("contains"), &[a, p]), I(b(a.contains(p.as_str()))), "contains({a:?}, {p:?})");
                assert_eq!(one(sql("starts_with"), &[a, p]), I(b(a.starts_with(p.as_str()))), "starts_with({a:?}, {p:?})");
                assert_eq!(one(sql("ends_with"), &[a, p]), I(b(a.ends_with(p.as_str()))), "ends_with({a:?}, {p:?})");
                assert_eq!(one(sql("index_of"), &[a, p]), I(index_of(a, p)), "index_of({a:?}, {p:?})");
                assert_eq!(one(sql("replace_all"), &[a, p, &"·"]), T(replace_all(a, p, "·")), "replace_all({a:?}, {p:?})");
                checked += 1;
            }
        }
        assert!(checked > 3000);
    }

    #[test]
    fn numbers() {
        assert_eq!(decimal(12345, 2), "123.45");
        assert_eq!(decimal(-5, 2), "-0.05");
        assert_eq!(decimal(7, 0), "7");
        assert_eq!(decimal(i64::MIN + 1, 18), "-9.223372036854775807");
        assert_eq!(decimal(NONE, 2), "");
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(-1, 3), -33);
        assert_eq!(percent(5, 0), NONE);
        assert_eq!(percent(i64::MAX, 1), NONE);
        assert_eq!(round_to(14, 5), 15);
        assert_eq!(round_to(12, 5), 10);
        assert_eq!(round_to(-12, 5), -10);
        assert_eq!(round_to(-13, 5), -15);
        assert_eq!(round_to(-12, 0), -12);
    }

    #[test]
    fn text_that_has_no_sql_form() {
        assert_eq!(repeat("ab", 3), "ababab");
        assert_eq!(repeat("ab", -1), "");
        assert!(repeat("x", i64::MAX).len() <= MAX_OUT);
        assert_eq!(pad("7", 3, "0", true), "007");
        assert_eq!(pad("日本", 3, "", false), "日本 ");
        assert_eq!(pad("long", 2, "-", true), "long");
        assert_eq!(capitalize("élan vital"), "Élan vital");
        assert_eq!(capitalize(""), "");
    }
}
