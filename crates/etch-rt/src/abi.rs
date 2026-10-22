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

/// A text's length in characters (as SQLite's length() counts them): an action's `max N` check.
/// # Safety: a live %Str.
#[no_mangle]
pub unsafe extern "C" fn etch_str_chars(s: *const Str) -> i64 {
    let s = &*s;
    if s.ptr.is_null() {
        return 0;
    }
    bytes(s.ptr, s.len).iter().filter(|&&b| b & 0xC0 != 0x80).count() as i64
}

// ---- queries: the generated loaders call SQLite's C API directly, through these few helpers

use rusqlite::ffi;

/// A connection and its prepared statements, by the generated query's id.
pub struct Db {
    conn: *mut ffi::sqlite3,
    stmts: Vec<*mut ffi::sqlite3_stmt>,
}

impl Db {
    /// # Safety: `conn` outlives the Db (a rusqlite Connection's handle).
    pub unsafe fn new(conn: *mut ffi::sqlite3) -> Db {
        Db { conn, stmts: vec![] }
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        for s in self.stmts.drain(..) {
            if !s.is_null() {
                unsafe { ffi::sqlite3_finalize(s) };
            }
        }
    }
}

/// The prepared statement for query `qid` (prepared on first use, kept). # Safety: live Db; SQL valid.
#[no_mangle]
pub unsafe extern "C" fn etch_stmt(db: *mut Db, qid: i64, sql: *const u8, len: i64) -> *mut ffi::sqlite3_stmt {
    let db = &mut *db;
    let k = qid as usize;
    if db.stmts.len() <= k {
        db.stmts.resize(k + 1, std::ptr::null_mut());
    }
    if db.stmts[k].is_null() {
        let mut st = std::ptr::null_mut();
        let rc = ffi::sqlite3_prepare_v3(db.conn, sql as *const _, len as i32, ffi::SQLITE_PREPARE_PERSISTENT as u32, &mut st, std::ptr::null_mut());
        if rc != ffi::SQLITE_OK {
            let msg = std::ffi::CStr::from_ptr(ffi::sqlite3_errmsg(db.conn)).to_string_lossy();
            panic!("a generated query doesn't prepare: {msg}: {}", std::str::from_utf8_unchecked(bytes(sql, len)));
        }
        db.stmts[k] = st;
    }
    db.stmts[k]
}

/// # Safety: a live statement on a row.
#[no_mangle]
pub unsafe extern "C" fn etch_col_i64(st: *mut ffi::sqlite3_stmt, i: i32) -> i64 {
    if ffi::sqlite3_column_type(st, i) == ffi::SQLITE_NULL {
        i64::MIN
    } else {
        ffi::sqlite3_column_int64(st, i)
    }
}

/// A text column, copied into the arena (none: a null pointer). # Safety: live statement, arena, dst.
#[no_mangle]
pub unsafe extern "C" fn etch_col_str(st: *mut ffi::sqlite3_stmt, i: i32, arena: *mut Arena, dst: *mut Str) {
    if ffi::sqlite3_column_type(st, i) == ffi::SQLITE_NULL {
        *dst = Str::NONE;
        return;
    }
    let p = ffi::sqlite3_column_text(st, i);
    let n = ffi::sqlite3_column_bytes(st, i) as usize;
    let copy = (*arena).take(n);
    std::ptr::copy_nonoverlapping(p, copy, n);
    *dst = Str { ptr: copy, len: n as i64 };
}

/// `size` zeroed bytes from the arena, 8-aligned. # Safety: a live arena.
#[no_mangle]
pub unsafe extern "C" fn etch_alloc(arena: *mut Arena, size: i64) -> *mut u8 {
    let p = (*arena).take(size as usize);
    std::ptr::write_bytes(p, 0, size as usize);
    p
}

/// A bump allocator for the records a build reads: chunks never move, so pointers into them stay
/// valid until `reset`.
#[derive(Default)]
pub struct Arena {
    chunks: Vec<Vec<u64>>,
    used: usize,
}

impl Arena {
    pub fn new() -> Arena {
        Arena::default()
    }
    fn take(&mut self, n: usize) -> *mut u8 {
        let words = n.div_ceil(8).max(1);
        let fits = self.chunks.last().map(|c| c.len() - self.used >= words).unwrap_or(false);
        if !fits {
            self.chunks.push(vec![0u64; words.max(8192)]);
            self.used = 0;
        }
        let c = self.chunks.last_mut().unwrap();
        let p = c[self.used..].as_mut_ptr() as *mut u8;
        self.used += words;
        p
    }
    /// Lets go of everything taken (keeps the first chunk).
    pub fn reset(&mut self) {
        self.chunks.truncate(1);
        self.used = 0;
    }
}

/// Marks a splice's place in a per-viewer register's bytes, and which splice goes there (its
/// site, in the high bits). # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_split(splits: *mut Vec<usize>, out: *mut Vec<u8>, site: i64) {
    (*splits).push((*out).len() | ((site as usize) << 48));
}

/// A split's offset and site.
pub fn split_at(s: usize) -> (usize, usize) {
    (s & ((1 << 48) - 1), s >> 48)
}

/// "a", "a and b", "a, b, and c", from items each ended by a NUL. # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_sentence(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let all = text(t);
    let items: Vec<&str> = if all.is_empty() { vec![] } else { all.strip_suffix('\0').unwrap_or(all).split('\0').collect() };
    let s = match items.len() {
        0 => String::new(),
        1 => items[0].to_string(),
        2 => format!("{} and {}", items[0], items[1]),
        n => format!("{}, and {}", items[..n - 1].join(", "), items[n - 1]),
    };
    put(buf(out), &s, esc);
}

/// Binds a text field's value (none: NULL). # Safety: live statement; `s` a live Str.
#[no_mangle]
pub unsafe extern "C" fn etch_bind_str(st: *mut ffi::sqlite3_stmt, i: i32, s: *const Str) {
    let s = &*s;
    if s.ptr.is_null() {
        ffi::sqlite3_bind_null(st, i);
    } else {
        ffi::sqlite3_bind_text(st, i, s.ptr as *const _, s.len as i32, ffi::SQLITE_TRANSIENT());
    }
}

// ---- the writer: what generated actions read and report

/// The writer's state an action reads: the time (one per batch), the next ids and seq, an arena
/// for the text it makes (tokens).
pub struct Writer {
    pub now: i64,
    pub seq: i64,
    next_ids: HashMap<String, i64>,
    arena: Arena,
}

impl Writer {
    pub fn new(seq: i64) -> Writer {
        Writer { now: 0, seq, next_ids: HashMap::new(), arena: Arena::default() }
    }
    /// A new batch: its time; the arena emptied.
    pub fn begin(&mut self, now: i64) {
        self.now = now;
        self.arena.reset();
    }
    /// Forgets the ids handed out since the last commit (a rolled-back batch).
    pub fn forget_ids(&mut self) {
        self.next_ids.clear();
    }
}

/// What an action's commit changes, for the writer to rebuild: (op, register or list, key, item).
/// 1 build register for a new row; 2 append item to a screen list; 3 rebuild register at key;
/// 4 rebuild every kept instance of a register; 5 rebuild a screen list at key.
#[derive(Default)]
pub struct Deltas {
    pub list: Vec<(i32, i64, i64, i64)>,
    /// (register, key, item id, list, field, value): set a field of an item in a kept record
    pub patches: Vec<(i64, i64, i64, i64, i64, i64)>,
    /// (register, row, list, field): kept records whose list holds an item referencing the row
    pub finds: Vec<(i64, i64, i64, i64)>,
}

/// # Safety: a live Deltas.
#[no_mangle]
pub unsafe extern "C" fn etch_delta_patch(d: *mut Deltas, reg: i64, key: i64, item: i64, list: i64, field: i64, value: i64) {
    (*d).patches.push((reg, key, item, list, field, value));
}

/// # Safety: a live Deltas.
#[no_mangle]
pub unsafe extern "C" fn etch_delta_find(d: *mut Deltas, reg: i64, row: i64, list: i64, field: i64) {
    let d = &mut *d;
    if !d.finds.contains(&(reg, row, list, field)) {
        d.finds.push((reg, row, list, field));
    }
}

/// `size` zeroed bytes from the writer's arena (a forwarded record), kept until the next batch.
/// # Safety: a live Writer.
#[no_mangle]
pub unsafe extern "C" fn etch_w_alloc(w: *mut Writer, size: i64) -> *mut u8 {
    etch_alloc(&mut (*w).arena, size)
}

/// # Safety: a live Deltas.
#[no_mangle]
pub unsafe extern "C" fn etch_delta(d: *mut Deltas, op: i32, a: i64, key: i64, item: i64) {
    let d = &mut *d;
    if !d.list.contains(&(op, a, key, item)) {
        d.list.push((op, a, key, item));
    }
}

/// # Safety: a live Writer.
#[no_mangle]
pub unsafe extern "C" fn etch_w_now(w: *mut Writer) -> i64 {
    (*w).now
}

/// A new seq. # Safety: a live Writer.
#[no_mangle]
pub unsafe extern "C" fn etch_w_seq(w: *mut Writer) -> i64 {
    (*w).seq += 1;
    (*w).seq
}

/// The seq last handed out. # Safety: a live Writer.
#[no_mangle]
pub unsafe extern "C" fn etch_w_seq_now(w: *mut Writer) -> i64 {
    (*w).seq
}

/// A new id for a row of `ent` (one more than the largest, read once). # Safety: live Writer, Db.
#[no_mangle]
pub unsafe extern "C" fn etch_w_id(w: *mut Writer, db: *mut Db, ent: *const u8, n: i64) -> i64 {
    let name = std::str::from_utf8_unchecked(bytes(ent, n)).to_string();
    let w = &mut *w;
    if !w.next_ids.contains_key(&name) {
        let sql = format!("SELECT coalesce(max(id), 0) + 1 FROM \"{name}\"");
        let st = etch_stmt(db, 1_000_000 + w.next_ids.len() as i64, sql.as_ptr(), sql.len() as i64);
        ffi::sqlite3_step(st);
        let v = ffi::sqlite3_column_int64(st, 0);
        ffi::sqlite3_reset(st);
        w.next_ids.insert(name.clone(), v);
    }
    let id = w.next_ids.get_mut(&name).unwrap();
    let v = *id;
    *id += 1;
    v
}

/// A random token of n URL-safe characters, in the writer's arena. # Safety: live Writer, dst.
#[no_mangle]
pub unsafe extern "C" fn etch_w_token(w: *mut Writer, n: i64, dst: *mut Str) {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let n = n as usize;
    let mut raw = vec![0u8; n];
    getrandom::getrandom(&mut raw).expect("random");
    let p = (*w).arena.take(n);
    for (i, b) in raw.iter().enumerate() {
        *p.add(i) = A[(*b & 63) as usize];
    }
    *dst = Str { ptr: p, len: n as i64 };
}

/// Runs a statement that returns no rows. # Safety: a live statement.
#[no_mangle]
pub unsafe extern "C" fn etch_exec(st: *mut ffi::sqlite3_stmt) {
    let rc = ffi::sqlite3_step(st);
    etch_done(st, rc);
}

/// Resets a statement; panics on an error (the writer rolls the batch back). # Safety: live statement.
#[no_mangle]
pub unsafe extern "C" fn etch_done(st: *mut ffi::sqlite3_stmt, rc: i32) {
    ffi::sqlite3_reset(st);
    if rc != ffi::SQLITE_DONE {
        let db = ffi::sqlite3_db_handle(st);
        let msg = std::ffi::CStr::from_ptr(ffi::sqlite3_errmsg(db)).to_string_lossy();
        panic!("a generated write failed ({rc}): {msg}");
    }
}

/// Copies text into the writer's arena (a forwarded record's text outlives the action's
/// arguments). # Safety: a live Writer; `src` a live Str; `dst` writable.
#[no_mangle]
pub unsafe extern "C" fn etch_w_copy(w: *mut Writer, src: *const Str, dst: *mut Str) {
    let s = &*src;
    if s.ptr.is_null() {
        *dst = Str::NONE;
        return;
    }
    let n = s.len as usize;
    let p = (*w).arena.take(n);
    std::ptr::copy_nonoverlapping(s.ptr, p, n);
    *dst = Str { ptr: p, len: s.len };
}
