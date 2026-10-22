//! The helpers generated code calls (C ABI). Linked under cross-language LTO, they inline into the
//! generated builders. They run at write time only: the fast path calls none of them.
//!
//! Buffers are `Vec<u8>`s passed by pointer; text is (pointer, length); none is a null pointer
//! for text and i64::MIN for numbers and times.

use crate::html;
use crate::sign::Signer;
use crate::time;
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    pub ptr: *const u8,
    pub len: i64,
}

impl Str {
    pub const NONE: Str = Str { ptr: std::ptr::null(), len: 0 };
    pub fn of(s: &str) -> Str {
        Str { ptr: s.as_ptr(), len: s.len() as i64 }
    }
    pub fn opt(s: Option<&str>) -> Str {
        s.map(Str::of).unwrap_or(Str::NONE)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct List {
    pub ptr: *const u8,
    pub len: i64,
}

impl List {
    pub fn of<T>(items: &[T]) -> List {
        List { ptr: items.as_ptr() as *const u8, len: items.len() as i64 }
    }
}

/// What the generated code reads that is fixed at start: the secret, the assets, the configs (in
/// the order the spec declares them).
pub struct Ctx {
    pub signer: Signer,
    pub assets: HashMap<String, String>,
    pub configs: Vec<String>,
}

#[inline]
unsafe fn buf<'a>(p: *mut Vec<u8>) -> &'a mut Vec<u8> {
    &mut *p
}
#[inline]
unsafe fn bytes<'a>(p: *const u8, n: i64) -> &'a [u8] {
    if p.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}
#[inline]
unsafe fn text<'a>(v: *mut Vec<u8>) -> &'a str {
    std::str::from_utf8_unchecked(&*v)
}
fn put(out: &mut Vec<u8>, s: &str, esc: i64) {
    if esc != 0 {
        html::esc(out, s)
    } else {
        out.extend_from_slice(s.as_bytes())
    }
}

/// # Safety: `out` is a live Vec; (p, n) a live byte range.
#[no_mangle]
pub unsafe extern "C" fn etch_put(out: *mut Vec<u8>, p: *const u8, n: i64) {
    buf(out).extend_from_slice(bytes(p, n));
}

/// # Safety: as etch_put; the bytes are UTF-8.
#[no_mangle]
pub unsafe extern "C" fn etch_esc(out: *mut Vec<u8>, p: *const u8, n: i64) {
    html::esc(buf(out), std::str::from_utf8_unchecked(bytes(p, n)));
}

/// # Safety: `v` is a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_clear(v: *mut Vec<u8>) {
    buf(v).clear();
}

/// # Safety: `out` is a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_int(out: *mut Vec<u8>, n: i64) {
    if n != i64::MIN {
        crate::out::push_int_signed(buf(out), n);
    }
}

/// The text in `t` as a JSON string literal. # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_json(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    put(buf(out), &html::json_string(text(t)), esc);
}

/// # Safety: `ctx` is a live Ctx; live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_sign(ctx: *const Ctx, out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let c = &*ctx;
    put(buf(out), &c.signer.token(text(t)), esc);
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_autolink(out: *mut Vec<u8>, t: *mut Vec<u8>) {
    buf(out).extend_from_slice(html::autolink(&html::esc_string(text(t))).as_bytes());
}

/// # Safety: `ctx` is a live Ctx; live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_asset(ctx: *const Ctx, out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let name = text(t);
    let c = &*ctx;
    let url = c.assets.get(name).cloned().unwrap_or_else(|| format!("/assets/{name}"));
    put(buf(out), &url, esc);
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_first_word(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    put(buf(out), text(t).split_whitespace().next().unwrap_or(""), esc);
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_initials(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let s: String = text(t).split_whitespace().filter_map(|w| w.chars().next()).collect();
    put(buf(out), &s, esc);
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_lower(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    put(buf(out), &text(t).to_ascii_lowercase(), esc);
}

/// # Safety: live Vecs and byte ranges.
#[no_mangle]
pub unsafe extern "C" fn etch_replace(out: *mut Vec<u8>, t: *mut Vec<u8>, f: *const u8, fl: i64, r: *const u8, rl: i64, esc: i64) {
    let from = std::str::from_utf8_unchecked(bytes(f, fl));
    let to = std::str::from_utf8_unchecked(bytes(r, rl));
    put(buf(out), &text(t).replace(from, to), esc);
}

/// kind 0: ISO UTC; 1: the date; 2: the clock (in the server's time zone). # Safety: a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_time(out: *mut Vec<u8>, kind: i64, ms: i64, esc: i64) {
    if ms == i64::MIN {
        return;
    }
    let s = match kind {
        0 => time::iso_utc(ms),
        1 => time::date(ms),
        _ => time::clock(ms),
    };
    put(buf(out), &s, esc);
}

/// # Safety: `ctx` is a live Ctx; a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_config(ctx: *const Ctx, out: *mut Vec<u8>, k: i64, esc: i64) {
    let c = &*ctx;
    put(buf(out), &c.configs[k as usize], esc);
}

/// # Safety: live byte ranges.
#[no_mangle]
pub unsafe extern "C" fn etch_str_eq(a: *const u8, an: i64, b: *const u8, bn: i64) -> bool {
    !a.is_null() && bytes(a, an) == bytes(b, bn)
}
