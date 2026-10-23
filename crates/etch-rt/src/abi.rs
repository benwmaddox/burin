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
    // as html::json_string, written as it goes (no allocation)
    let o = buf(out);
    put(o, "\"", esc);
    for ch in text(t).chars() {
        match ch {
            '"' => put(o, "\\\"", esc),
            '\\' => put(o, "\\\\", esc),
            '\n' => put(o, "\\n", esc),
            '\r' => put(o, "\\r", esc),
            '\t' => put(o, "\\t", esc),
            '\u{2028}' => put(o, "\\u2028", esc),
            '\u{2029}' => put(o, "\\u2029", esc),
            c if (c as u32) < 0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let u = [b'\\', b'u', b'0', b'0', HEX[(c as usize) >> 4], HEX[(c as usize) & 15]];
                put(o, std::str::from_utf8_unchecked(&u), esc);
            }
            c => put(o, c.encode_utf8(&mut [0u8; 4]), esc),
        }
    }
    put(o, "\"", esc);
}

/// # Safety: `ctx` is a live Ctx; live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_sign(ctx: *const Ctx, out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    // base64url and a dot: nothing to escape
    let _ = esc;
    (*ctx).signer.token_into(text(t), buf(out));
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
    match (*ctx).assets.get(name) {
        Some(url) => put(buf(out), url, esc),
        None => {
            put(buf(out), "/assets/", esc);
            put(buf(out), name, esc);
        }
    }
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_first_word(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    put(buf(out), text(t).split_whitespace().next().unwrap_or(""), esc);
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_initials(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let o = buf(out);
    for c in text(t).split_whitespace().filter_map(|w| w.chars().next()) {
        put(o, c.encode_utf8(&mut [0u8; 4]), esc);
    }
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_upper(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let o = buf(out);
    for c in text(t).chars().flat_map(char::to_uppercase) {
        put(o, c.encode_utf8(&mut [0u8; 4]), esc);
    }
}

/// `choose`: the CRC-32 of a text, modulo n (Campfire picks an avatar's color this way).
/// # Safety: a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_crc_mod(t: *mut Vec<u8>, n: i64) -> i64 {
    (crc32fast::hash(&*t) as i64).rem_euclid(n.max(1))
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_lower(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let o = buf(out);
    for c in text(t).chars().map(|c| c.to_ascii_lowercase()) {
        put(o, c.encode_utf8(&mut [0u8; 4]), esc);
    }
}

/// # Safety: live Vecs and byte ranges.
#[no_mangle]
pub unsafe extern "C" fn etch_replace(out: *mut Vec<u8>, t: *mut Vec<u8>, f: *const u8, fl: i64, r: *const u8, rl: i64, esc: i64) {
    let from = std::str::from_utf8_unchecked(bytes(f, fl));
    let to = std::str::from_utf8_unchecked(bytes(r, rl));
    put(buf(out), &text(t).replace(from, to), esc);
}

/// A time or number as text. kind 0: ISO UTC; 1: the date; 2: the clock (in the server's time
/// zone); 3: a duration ("2h 5m"); 4: an Int grouped by thousands ("1,234"). # Safety: a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_time(out: *mut Vec<u8>, kind: i64, ms: i64, esc: i64) {
    if ms == i64::MIN {
        return;
    }
    let s = match kind {
        0 => time::iso_utc(ms),
        1 => time::date(ms),
        2 => time::clock(ms),
        3 => time::span(ms),
        _ => time::grouped(ms),
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

/// An integer parameter; none (i64::MIN) as NULL. # Safety: a live statement.
#[no_mangle]
pub unsafe extern "C" fn etch_bind_i64(st: *mut ffi::sqlite3_stmt, i: i32, v: i64) {
    if v == i64::MIN {
        ffi::sqlite3_bind_null(st, i);
    } else {
        ffi::sqlite3_bind_int64(st, i, v);
    }
}

/// `sign in`: the new session's token, for the reply's cookie. # Safety: live writer and text.
#[no_mangle]
pub unsafe extern "C" fn etch_w_sign_in(w: *mut Writer, token: *const Str) {
    let t = &*token;
    (*w).signed_in = Some(bytes(t.ptr, t.len).to_vec());
}

/// A for each's rows: every id the statement returns (its first column), in the writer's arena,
/// read before the body runs (the body may write the table read). # Safety: live writer, statement.
#[no_mangle]
pub unsafe extern "C" fn etch_w_ids(w: *mut Writer, st: *mut ffi::sqlite3_stmt) -> List {
    let mut ids: Vec<i64> = vec![];
    while ffi::sqlite3_step(st) == ffi::SQLITE_ROW {
        ids.push(ffi::sqlite3_column_int64(st, 0));
    }
    ffi::sqlite3_reset(st);
    let p = (*w).arena.take(ids.len() * 8) as *mut i64;
    std::ptr::copy_nonoverlapping(ids.as_ptr(), p, ids.len());
    List { ptr: p as *const u8, len: ids.len() as i64 }
}

/// `go screen(row)`: where the action goes (the route around the row's id). # Safety: live
/// writer and byte ranges.
#[no_mangle]
pub unsafe extern "C" fn etch_w_go(w: *mut Writer, pre: *const u8, pre_n: i64, id: i64, post: *const u8, post_n: i64) {
    let mut s = String::from_utf8_lossy(bytes(pre, pre_n)).into_owned();
    s.push_str(&id.to_string());
    s.push_str(&String::from_utf8_lossy(bytes(post, post_n)));
    (*w).go = Some(s);
}

/// `sign out`: the reply clears the session's cookie. # Safety: a live writer.
#[no_mangle]
pub unsafe extern "C" fn etch_w_sign_out(w: *mut Writer) {
    (*w).signed_out = true;
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

/// A number an action computed in SQL: none unless it is an INTEGER (SQLite turns an overflow
/// into a REAL; a fragment's code gives none for it). # Safety: a live statement on a row.
#[no_mangle]
pub unsafe extern "C" fn etch_col_int(st: *mut ffi::sqlite3_stmt, i: i32) -> i64 {
    if ffi::sqlite3_column_type(st, i) == ffi::SQLITE_INTEGER {
        ffi::sqlite3_column_int64(st, i)
    } else {
        i64::MIN
    }
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
    /// the chunk being filled, and the words used in it
    cur: usize,
    used: usize,
}

impl Arena {
    pub fn new() -> Arena {
        Arena::default()
    }
    fn take(&mut self, n: usize) -> *mut u8 {
        let words = n.div_ceil(8).max(1);
        // the next chunk with room (chunks are kept across resets: a reused arena allocates
        // nothing once it has grown to its largest use)
        while self.cur < self.chunks.len() && self.chunks[self.cur].len() - self.used < words {
            self.cur += 1;
            self.used = 0;
        }
        if self.cur == self.chunks.len() {
            self.chunks.push(vec![0u64; words.max(8192)]);
            self.used = 0;
        }
        let c = &mut self.chunks[self.cur];
        let p = c[self.used..].as_mut_ptr() as *mut u8;
        self.used += words;
        p
    }
    /// Lets go of everything taken (the chunks are kept, for the next use).
    pub fn reset(&mut self) {
        self.cur = 0;
        self.used = 0;
    }
}

/// Marks where a cached piece goes in a per-request fragment's bytes: its place, its kind and its
/// key (two entries: the place with the kind in the high bits, then the key). # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_piece(pieces: *mut Vec<usize>, out: *mut Vec<u8>, kind: i64, key: i64) {
    (*pieces).push((*out).len() | ((kind as usize) << 48));
    (*pieces).push(key as usize);
}

/// A split's offset and site.
pub fn split_at(s: usize) -> (usize, usize) {
    (s & ((1 << 48) - 1), s >> 48)
}

/// "a", "a and b", "a, b, and c", from items each ended by a NUL. # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_sentence(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let all = text(t);
    let all = all.strip_suffix('\0').unwrap_or(all);
    let n = if all.is_empty() { 0 } else { all.split('\0').count() };
    let o = buf(out);
    for (i, item) in all.split('\0').enumerate().take(n) {
        if i > 0 {
            put(o, if n == 2 { " and " } else if i == n - 1 { ", and " } else { ", " }, 0);
        }
        put(o, item, esc);
    }
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
    /// the action signed a viewer in: the new session's token (its cookie)
    pub signed_in: Option<Vec<u8>>,
    /// the action signed the request's session out (its cookie cleared)
    pub signed_out: bool,
    /// where the action goes (`go`)
    pub go: Option<String>,
    /// an external function's text result, before it's kept in the arena
    ext: Vec<u8>,
}

impl Writer {
    pub fn new(seq: i64) -> Writer {
        Writer { now: 0, seq, next_ids: HashMap::new(), arena: Arena::default(), signed_in: None, signed_out: false, go: None, ext: Vec::new() }
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

thread_local! {
    /// A generated write that failed (a constraint): the writer rolls the action back.
    static WRITE_FAILED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The failure of the action just run, if a write of it failed (and forgets it).
pub fn take_write_failure() -> Option<String> {
    WRITE_FAILED.with(|f| f.borrow_mut().take())
}

/// Resets a statement; an error is kept for the writer (which rolls the action back).
/// # Safety: a live statement.
#[no_mangle]
pub unsafe extern "C" fn etch_done(st: *mut ffi::sqlite3_stmt, rc: i32) {
    ffi::sqlite3_reset(st);
    if rc != ffi::SQLITE_DONE {
        let db = ffi::sqlite3_db_handle(st);
        let msg = std::ffi::CStr::from_ptr(ffi::sqlite3_errmsg(db)).to_string_lossy().into_owned();
        WRITE_FAILED.with(|f| {
            f.borrow_mut().get_or_insert(format!("{rc}: {msg}"));
        });
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

// ---- external functions (`extern fn`): include/etch.h

/// Appends text to an external function's result (what C code calls; Rust code may too).
/// Text that isn't UTF-8 is replaced where it's used. # Safety: `out` the buffer the function
/// was given; `p` readable for `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn etch_ext_write(out: *mut Vec<u8>, p: *const u8, n: i64) {
    if n > 0 && !p.is_null() {
        buf(out).extend_from_slice(std::slice::from_raw_parts(p, n as usize));
    }
}

/// A buffer's bytes and length, for an external function's text argument. # Safety: a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_vec_ptr(v: *mut Vec<u8>) -> *const u8 {
    (*v).as_ptr()
}
/// # Safety: a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_vec_len(v: *mut Vec<u8>) -> i64 {
    (*v).len() as i64
}

/// An external function's text result, into the page (escaped when `esc`): what isn't UTF-8 is
/// replaced (U+FFFD), so the page stays valid text. # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_emit(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    let o = buf(out);
    match std::str::from_utf8(&*t) {
        Ok(s) => put(o, s, esc),
        Err(_) => put(o, &String::from_utf8_lossy(&*t), esc),
    }
}

/// The writer's buffer for an external function's text result, emptied. # Safety: a live Writer.
#[no_mangle]
pub unsafe extern "C" fn etch_w_ext_out(w: *mut Writer) -> *mut Vec<u8> {
    let w = &mut *w;
    w.ext.clear();
    &mut w.ext
}

/// The external function's text result, kept in the writer's arena as the action's text (what
/// isn't UTF-8 replaced). # Safety: a live Writer; `dst` writable.
#[no_mangle]
pub unsafe extern "C" fn etch_w_ext_take(w: *mut Writer, dst: *mut Str) {
    let w = &mut *w;
    if std::str::from_utf8(&w.ext).is_err() {
        w.ext = String::from_utf8_lossy(&w.ext).into_owned().into_bytes();
    }
    let n = w.ext.len();
    let p = w.arena.take(n.max(1));
    std::ptr::copy_nonoverlapping(w.ext.as_ptr(), p, n);
    *dst = Str { ptr: p, len: n as i64 };
}
